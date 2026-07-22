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
apt-get install -y caddy curl docker-compose-v2 docker.io gzip openssl unattended-upgrades ufw

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

install -d -m 0755 /etc/wareboxes /opt/wareboxes/runtime /opt/wareboxes/releases
install -d -m 0750 -o deploy -g deploy /var/lib/wareboxes/uploads
install -d -m 0755 /opt/wareboxes/bootstrap/site

if [ ! -f /etc/wareboxes/postgres_password ]; then
  openssl rand -hex 32 > /etc/wareboxes/postgres_password
fi
chmod 0600 /etc/wareboxes/postgres_password

if [ ! -f /etc/wareboxes/wareboxes.env ]; then
  database_password="$(cat /etc/wareboxes/postgres_password)"
  cat > /etc/wareboxes/wareboxes.env <<EOF
DATABASE_URL=postgres://wareboxes:${database_password}@127.0.0.1:5432/wareboxes
BIND_ADDR=127.0.0.1:8080
ALLOW_PUBLIC_REGISTRATION=false
CORS_ALLOWED_ORIGINS=
MAX_REQUEST_BODY_BYTES=1048576
RUST_LOG=info,wareboxes_server=info
EOF
fi
chown root:wareboxes /etc/wareboxes/wareboxes.env
chmod 0640 /etc/wareboxes/wareboxes.env

printf 'WAREBOXES_SITE_ADDRESS=%s\n' "$site_address" > /etc/wareboxes/caddy.env
chmod 0644 /etc/wareboxes/caddy.env

install -m 0644 "$script_dir/Caddyfile" /etc/caddy/Caddyfile
install -m 0644 "$script_dir/postgres.compose.yml" /opt/wareboxes/runtime/postgres.compose.yml
install -m 0644 "$script_dir/wareboxes.service" /etc/systemd/system/wareboxes.service
install -m 0755 "$script_dir/wareboxes-deploy" /usr/local/sbin/wareboxes-deploy

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
docker compose -f /opt/wareboxes/runtime/postgres.compose.yml up -d

systemctl daemon-reload
systemctl enable wareboxes.service
caddy validate --config /etc/caddy/Caddyfile
systemctl enable caddy.service
systemctl restart caddy.service
systemctl reload ssh.service
curl --fail --silent --show-error --retry 10 --retry-delay 1 "$site_address" >/dev/null

echo "Wareboxes host provisioning complete."
