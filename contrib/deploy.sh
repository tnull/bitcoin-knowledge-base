#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

# === Configuration ===
VPS_HOST="root@bitcoinknowledge.dev"

prompt_if_unset() {
    local name="$1" label="$2"
    if [[ ! -v "$name" ]]; then
        if ! IFS= read -r -s -p "$label: " "${name?}"; then
            printf '\nERROR: Set %s in the environment or enter it at the prompt.\n' "$name" >&2
            exit 1
        fi
        printf '\n' >&2
    fi
}

prompt_if_unset GITHUB_TOKEN 'GitHub token'
prompt_if_unset FORGEJO_TOKEN 'Forgejo token'
prompt_if_unset BKB_ADMIN_PASSWORD 'Admin password (empty disables admin endpoints)'
for name in GITHUB_TOKEN FORGEJO_TOKEN; do
    if [[ -z "${!name}" ]]; then
        printf 'ERROR: %s must not be empty.\n' "$name" >&2
        exit 1
    fi
done

# Match Cargo's output location, keeping default builds outside the checkout.
branch="$(git rev-parse --abbrev-ref HEAD)"
branch="${branch//\//-}"
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$(mktemp -d "/tmp/cargo-target-bkb-${branch}-deploy.XXXXXX")}"
export CARGO_TARGET_DIR
BINARY="$CARGO_TARGET_DIR/release/bkb-server"

# === Build ===
echo "==> Building bkb-server..."
BKB_GIT_HASH="$(git rev-parse --short HEAD)"
export BKB_GIT_HASH
cargo build --locked --release -p bkb-server

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
mkdir -p /opt/bkb/data /opt/bkb/cache
chown -R bkb:bkb /opt/bkb
SETUP

echo "==> Uploading binary and service file..."
scp "$BINARY" "${VPS_HOST}:/opt/bkb/bkb-server.new"

# EnvironmentFile uses double-quoted values without shell expansion.
# Stream secrets through SSH stdin, never through remote command arguments.
write_variable() {
    local name="$1" value="${!1}"
    value="${value//\\/\\\\}"
    value="${value//\"/\\\"}"
    printf '%s="%s"\n' "$name" "$value"
}

write_environment() {
    write_variable GITHUB_TOKEN
    write_variable FORGEJO_TOKEN
    # Omit an empty password: setting it would enable passwordless admin auth.
    if [[ -n "$BKB_ADMIN_PASSWORD" ]]; then
        write_variable BKB_ADMIN_PASSWORD
    fi
}

echo "==> Installing service credentials..."
write_environment | ssh "$VPS_HOST" 'set -eu
umask 077
env_file=$(mktemp /etc/bkb-server.env.XXXXXX)
trap '\''rm -- "$env_file"'\'' EXIT
cat > "$env_file"
mv "$env_file" /etc/bkb-server.env
trap - EXIT'

scp "contrib/bkb-server.service" "${VPS_HOST}:/etc/systemd/system/bkb-server.service"

echo "==> Installing..."
ssh "$VPS_HOST" bash <<'REMOTE'
set -euo pipefail

# Swap binary
mv /opt/bkb/bkb-server.new /opt/bkb/bkb-server
chmod +x /opt/bkb/bkb-server

# Reload and restart
systemctl daemon-reload
systemctl restart bkb-server
systemctl status --no-pager bkb-server

echo "==> bkb-server deployed and running"
REMOTE

echo "==> Done. Check: https://bitcoinknowledge.dev/health"
