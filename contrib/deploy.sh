#!/usr/bin/env bash
set -euo pipefail

# === Configuration ===
VPS_HOST="root@bitcoinknowledge.dev"
GITHUB_TOKEN="${GITHUB_TOKEN:?Set GITHUB_TOKEN env var}"
BINARY="target/release/bkb-server"

# === Build ===
echo "==> Building bkb-server..."
cargo build --release -p bkb-server

if [ ! -f "$BINARY" ]; then
    echo "ERROR: Binary not found at $BINARY"
    exit 1
fi

echo "==> Binary size: $(du -h "$BINARY" | cut -f1)"

# === Deploy ===
echo "==> Preparing VPS..."
ssh "$VPS_HOST" bash <<'SETUP'
set -euo pipefail
if ! id bkb &>/dev/null; then
    useradd --system --no-create-home --shell /usr/sbin/nologin bkb
    echo "Created bkb user"
fi
mkdir -p /opt/bkb/data
chown -R bkb:bkb /opt/bkb
SETUP

echo "==> Uploading binary..."
scp "$BINARY" "${VPS_HOST}:/opt/bkb/bkb-server.new"

echo "==> Installing..."
ssh "$VPS_HOST" bash -s "$GITHUB_TOKEN" <<'REMOTE'
set -euo pipefail
GITHUB_TOKEN="$1"

# Swap binary
mv /opt/bkb/bkb-server.new /opt/bkb/bkb-server
chmod +x /opt/bkb/bkb-server

# Update GITHUB_TOKEN in service file
if [ -n "$GITHUB_TOKEN" ]; then
    sed -i "s/^Environment=GITHUB_TOKEN=.*/Environment=GITHUB_TOKEN=${GITHUB_TOKEN}/" \
        /etc/systemd/system/bkb-server.service
fi

# Reload and restart
systemctl daemon-reload
systemctl restart bkb-server
systemctl status --no-pager bkb-server

echo "==> bkb-server deployed and running"
REMOTE

echo "==> Done. Check: https://bitcoinknowledge.dev/health"
