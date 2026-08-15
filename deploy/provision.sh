#!/usr/bin/env bash
set -euo pipefail

if [ "$(id -u)" -ne 0 ] || [ "$#" -ne 2 ]; then
  echo "usage: sudo deploy/provision.sh '<deploy-public-key>' '<site-address>'" >&2
  exit 2
fi

deploy_public_key="$1"
site_address="$2"
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [[ "$deploy_public_key" != ssh-*' '* ]] || [ -z "$site_address" ]; then
  echo "a valid SSH public key and site address are required" >&2
  exit 2
fi

export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get upgrade -y
apt-get install -y caddy curl docker-compose-v2 docker.io gzip openssl restic unattended-upgrades ufw

systemctl enable --now docker.service

if ! id deploy >/dev/null 2>&1; then
  useradd --create-home --shell /bin/bash deploy
fi
if ! id wareboxes >/dev/null 2>&1; then
  useradd --system --home /nonexistent --shell /usr/sbin/nologin wareboxes
fi

install -d -m 0700 -o deploy -g deploy /home/deploy/.ssh
printf '%s\n' "$deploy_public_key" > /home/deploy/.ssh/authorized_keys
chown deploy:deploy /home/deploy/.ssh/authorized_keys
chmod 0600 /home/deploy/.ssh/authorized_keys

install -d -m 0755 /etc/wareboxes /opt/wareboxes/runtime /opt/wareboxes/runtime/postgres-init /opt/wareboxes/releases
install -d -m 0750 -o deploy -g deploy /var/lib/wareboxes/uploads
install -d -m 0755 /opt/wareboxes/bootstrap/site
install -d -m 0700 /var/backups/wareboxes /var/backups/wareboxes/restic /var/cache/wareboxes /var/cache/wareboxes/restic

if [ ! -f /etc/wareboxes/postgres_admin_password ]; then
  openssl rand -hex 32 > /etc/wareboxes/postgres_admin_password
fi
if [ ! -f /etc/wareboxes/postgres_app_password ]; then
  openssl rand -hex 32 > /etc/wareboxes/postgres_app_password
fi
if [ ! -f /etc/wareboxes/restic_password ]; then
  openssl rand -hex 32 > /etc/wareboxes/restic_password
fi
chmod 0600 \
  /etc/wareboxes/postgres_admin_password \
  /etc/wareboxes/postgres_app_password \
  /etc/wareboxes/restic_password

if [ ! -f /etc/wareboxes/backup.env ]; then
  backup_host="$(hostname -f)"
  cat > /etc/wareboxes/backup.env <<EOF
RESTIC_REPOSITORY=/var/backups/wareboxes/restic
RESTIC_PASSWORD_FILE=/etc/wareboxes/restic_password
RESTIC_CACHE_DIR=/var/cache/wareboxes/restic
WAREBOXES_POSTGRES_COMPOSE_FILE=/opt/wareboxes/runtime/postgres.compose.yml
WAREBOXES_POSTGRES_SERVICE=postgres
WAREBOXES_POSTGRES_DATABASE=wareboxes
WAREBOXES_POSTGRES_USER=wareboxes_admin
WAREBOXES_POSTGRES_RUNTIME_ROLE=wareboxes_app
WAREBOXES_BACKUP_HOST=$backup_host
WAREBOXES_BACKUP_TAG=wareboxes-postgres
WAREBOXES_BACKUP_DUMP_PATH=/wareboxes/postgres.dump
WAREBOXES_BACKUP_KEEP_DAILY=7
WAREBOXES_BACKUP_KEEP_WEEKLY=5
WAREBOXES_BACKUP_KEEP_MONTHLY=12
WAREBOXES_COMMAND_ARCHIVE_TAG=wareboxes-command-archive
WAREBOXES_COMMAND_ARCHIVE_AFTER_DAYS=90
WAREBOXES_COMMAND_ARCHIVE_KEEP_MONTHLY=24
WAREBOXES_RESTORE_POSTGRES_IMAGE=postgres:16-bookworm
WAREBOXES_RESTORE_MAX_SECONDS=3600
WAREBOXES_POSTGRES_INIT_SCRIPT=/opt/wareboxes/runtime/postgres-init/001-create-app-role.sh
EOF
fi
chown root:root /etc/wareboxes/backup.env
chmod 0600 /etc/wareboxes/backup.env

if [ ! -f /etc/wareboxes/wareboxes.env ]; then
  cat > /etc/wareboxes/wareboxes.env <<EOF
BIND_ADDR=127.0.0.1:8080
ALLOW_PUBLIC_REGISTRATION=false
CORS_ALLOWED_ORIGINS=
MAX_REQUEST_BODY_BYTES=1048576
MAX_IN_FLIGHT_REQUESTS=256
REQUEST_RATE_LIMIT_PER_SECOND=1000
LOGIN_RATE_LIMIT_PER_MINUTE=60
REQUEST_TIMEOUT_SECONDS=30
WEB_SESSION_ABSOLUTE_TTL_SECONDS=43200
WEB_SESSION_IDLE_TTL_SECONDS=1800
SECURE_WEB_SESSION_COOKIE=true
RUST_LOG=info,wareboxes_server=info,sqlx::query=error
LOG_FORMAT=json
EOF
fi

set_env_value() {
  local key="$1"
  local value="$2"
  local env_file="$3"
  local replacement

  replacement="$(mktemp)"
  awk -v key="$key" -v value="$value" '
    BEGIN { found = 0 }
    index($0, key "=") == 1 {
      if (!found) {
        print key "=" value
        found = 1
      }
      next
    }
    { print }
    END {
      if (!found) {
        print key "=" value
      }
    }
  ' "$env_file" > "$replacement"
  install -m 0640 "$replacement" "$env_file"
  rm -f "$replacement"
}

ensure_env_value() {
  local key="$1"
  local value="$2"
  local env_file="$3"
  if ! grep -q "^${key}=" "$env_file"; then
    set_env_value "$key" "$value" "$env_file"
  fi
}

database_admin_password="$(cat /etc/wareboxes/postgres_admin_password)"
database_app_password="$(cat /etc/wareboxes/postgres_app_password)"
set_env_value \
  DATABASE_URL \
  "postgres://wareboxes_app:${database_app_password}@127.0.0.1:5432/wareboxes" \
  /etc/wareboxes/wareboxes.env
set_env_value \
  MIGRATION_DATABASE_URL \
  "postgres://wareboxes_admin:${database_admin_password}@127.0.0.1:5432/wareboxes" \
  /etc/wareboxes/wareboxes.env
set_env_value LEPTOS_OUTPUT_NAME wareboxes-web /etc/wareboxes/wareboxes.env
set_env_value LEPTOS_SITE_ROOT /opt/wareboxes/current/site /etc/wareboxes/wareboxes.env
set_env_value LEPTOS_SITE_PKG_DIR pkg /etc/wareboxes/wareboxes.env
set_env_value LEPTOS_ENV PROD /etc/wareboxes/wareboxes.env
set_env_value WEB_SESSION_ABSOLUTE_TTL_SECONDS 43200 /etc/wareboxes/wareboxes.env
set_env_value WEB_SESSION_IDLE_TTL_SECONDS 1800 /etc/wareboxes/wareboxes.env
set_env_value SECURE_WEB_SESSION_COOKIE true /etc/wareboxes/wareboxes.env
ensure_env_value MAX_IN_FLIGHT_REQUESTS 256 /etc/wareboxes/wareboxes.env
ensure_env_value REQUEST_RATE_LIMIT_PER_SECOND 1000 /etc/wareboxes/wareboxes.env
ensure_env_value LOGIN_RATE_LIMIT_PER_MINUTE 60 /etc/wareboxes/wareboxes.env
ensure_env_value REQUEST_TIMEOUT_SECONDS 30 /etc/wareboxes/wareboxes.env
ensure_env_value LOG_FORMAT json /etc/wareboxes/wareboxes.env
ensure_env_value WAREBOXES_COMMAND_ARCHIVE_TAG wareboxes-command-archive /etc/wareboxes/backup.env
ensure_env_value WAREBOXES_COMMAND_ARCHIVE_AFTER_DAYS 90 /etc/wareboxes/backup.env
ensure_env_value WAREBOXES_COMMAND_ARCHIVE_KEEP_MONTHLY 24 /etc/wareboxes/backup.env
chmod 0600 /etc/wareboxes/backup.env
chown root:wareboxes /etc/wareboxes/wareboxes.env
chmod 0640 /etc/wareboxes/wareboxes.env

printf 'WAREBOXES_SITE_ADDRESS=%s\n' "$site_address" > /etc/wareboxes/caddy.env
chmod 0644 /etc/wareboxes/caddy.env

install -m 0644 "$script_dir/Caddyfile" /etc/caddy/Caddyfile
install -m 0644 "$script_dir/postgres.compose.yml" /opt/wareboxes/runtime/postgres.compose.yml
install -m 0755 "$script_dir/postgres-init/001-create-app-role.sh" /opt/wareboxes/runtime/postgres-init/001-create-app-role.sh
install -m 0644 "$script_dir/runtime-version" /etc/wareboxes/runtime-version
install -m 0644 "$script_dir/wareboxes.service" /etc/systemd/system/wareboxes.service
install -m 0644 "$script_dir/wareboxes-worker.service" /etc/systemd/system/wareboxes-worker.service
install -m 0644 "$script_dir/wareboxes-backup.service" /etc/systemd/system/wareboxes-backup.service
install -m 0644 "$script_dir/wareboxes-backup.timer" /etc/systemd/system/wareboxes-backup.timer
install -m 0644 "$script_dir/wareboxes-command-archive.service" /etc/systemd/system/wareboxes-command-archive.service
install -m 0644 "$script_dir/wareboxes-command-archive.timer" /etc/systemd/system/wareboxes-command-archive.timer
install -m 0644 "$script_dir/wareboxes-restore-drill.service" /etc/systemd/system/wareboxes-restore-drill.service
install -m 0644 "$script_dir/wareboxes-restore-drill.timer" /etc/systemd/system/wareboxes-restore-drill.timer
install -m 0755 "$script_dir/wareboxes-deploy" /usr/local/sbin/wareboxes-deploy
install -m 0755 "$script_dir/wareboxes-backup" /usr/local/sbin/wareboxes-backup
install -m 0755 "$script_dir/wareboxes-archive-commands" /usr/local/sbin/wareboxes-archive-commands
install -m 0755 "$script_dir/wareboxes-restore-drill" /usr/local/sbin/wareboxes-restore-drill
install -m 0755 "$script_dir/wareboxes-restore-postgres" /usr/local/sbin/wareboxes-restore-postgres

set -a
# shellcheck disable=SC1091
. /etc/wareboxes/backup.env
set +a
if [ "$RESTIC_REPOSITORY" = /var/backups/wareboxes/restic ] \
  && [ ! -f /var/backups/wareboxes/restic/config ]; then
  restic init
fi

install -d -m 0755 /etc/systemd/system/caddy.service.d
cat > /etc/systemd/system/caddy.service.d/wareboxes.conf <<'EOF'
[Service]
EnvironmentFile=/etc/wareboxes/caddy.env
EOF

cat > /etc/sudoers.d/wareboxes-deploy <<'EOF'
deploy ALL=(root) NOPASSWD: /usr/local/sbin/wareboxes-deploy *
EOF
chmod 0440 /etc/sudoers.d/wareboxes-deploy
visudo --check --file=/etc/sudoers.d/wareboxes-deploy

cat > /etc/ssh/sshd_config.d/60-wareboxes.conf <<'EOF'
KbdInteractiveAuthentication no
PasswordAuthentication no
PermitRootLogin prohibit-password
X11Forwarding no
EOF
install -d -m 0755 /run/sshd
sshd -t

if [ ! -f /swapfile ]; then
  fallocate -l 2G /swapfile
  chmod 0600 /swapfile
  mkswap /swapfile
  swapon /swapfile
  printf '/swapfile none swap sw 0 0\n' >> /etc/fstab
fi

cat > /opt/wareboxes/bootstrap/site/index.html <<'EOF'
<!doctype html><html lang="en"><meta charset="utf-8"><title>Wareboxes</title><body>Wareboxes is awaiting its first deployment.</body></html>
EOF
ln -sfn /opt/wareboxes/bootstrap /opt/wareboxes/current

ufw allow OpenSSH
ufw allow 80/tcp
ufw allow 443/tcp
ufw --force enable

docker compose -f /opt/wareboxes/runtime/postgres.compose.yml pull
postgres_group_id="$(docker run --rm --entrypoint id postgres:16-bookworm -g postgres)"
if [[ ! "$postgres_group_id" =~ ^[0-9]+$ ]]; then
  echo "could not resolve the PostgreSQL container group ID" >&2
  exit 1
fi
chown root:"$postgres_group_id" \
  /etc/wareboxes/postgres_admin_password \
  /etc/wareboxes/postgres_app_password
chmod 0640 \
  /etc/wareboxes/postgres_admin_password \
  /etc/wareboxes/postgres_app_password
docker compose -f /opt/wareboxes/runtime/postgres.compose.yml up -d

systemctl daemon-reload
systemctl enable wareboxes.service
systemctl enable --now \
  wareboxes-backup.timer \
  wareboxes-command-archive.timer \
  wareboxes-restore-drill.timer
caddy validate --config /etc/caddy/Caddyfile
systemctl enable caddy.service
systemctl restart caddy.service
systemctl reload ssh.service
curl --fail --silent --show-error --retry 10 --retry-delay 1 "$site_address" >/dev/null

echo "Wareboxes host provisioning complete."
