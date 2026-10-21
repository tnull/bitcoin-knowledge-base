#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

# === Configuration ===
VPS_HOST="root@bitcoinknowledge.dev"
DEPLOY_HEALTH_TIMEOUT="${DEPLOY_HEALTH_TIMEOUT:-600}"
if [[ ! "$DEPLOY_HEALTH_TIMEOUT" =~ ^[1-9][0-9]*$ ]]; then
    echo 'ERROR: DEPLOY_HEALTH_TIMEOUT must be a positive number of seconds.' >&2
    exit 1
fi

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
if ! command -v curl >/dev/null; then
    echo 'ERROR: Install curl on the VPS before deploying; it is required for health checks.' >&2
    exit 1
fi
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
ssh "$VPS_HOST" bash -s -- "$DEPLOY_HEALTH_TIMEOUT" <<'REMOTE'
set -euo pipefail

startup_diagnostics() {
    systemctl status --no-pager --full bkb-server || true
    journalctl -u bkb-server -n 80 --no-pager || true
}

# Swap binary
mv /opt/bkb/bkb-server.new /opt/bkb/bkb-server
chmod +x /opt/bkb/bkb-server

# Reload and restart
systemctl daemon-reload
if ! systemctl restart bkb-server; then
    startup_diagnostics
    exit 1
fi

echo "==> Waiting up to ${1}s for the API (the first database upgrade may take several minutes)..."
deadline=$((SECONDS + $1))
next_report=$((SECONDS + 30))
until curl --fail --silent --connect-timeout 2 --max-time 10 \
    --output /dev/null http://127.0.0.1:3000/health; do
    if ! systemctl is-active --quiet bkb-server; then
        echo 'ERROR: Service stopped or restarted before the API became ready.' >&2
        startup_diagnostics
        exit 1
    fi
    if (( SECONDS >= deadline )); then
        echo 'ERROR: API readiness timed out. The service is left running; an upgrade may still be in progress.' >&2
        startup_diagnostics
        exit 1
    fi
    if (( SECONDS >= next_report )); then
        echo '==> Still waiting for the API; follow progress with journalctl -fu bkb-server.'
        next_report=$((SECONDS + 30))
    fi
    sleep 2
done
systemctl status --no-pager bkb-server

echo "==> Local API health check passed"
REMOTE

echo "==> Checking public API..."
if ! curl --fail --silent --show-error --connect-timeout 5 --max-time 30 \
    --output /dev/null https://bitcoinknowledge.dev/health; then
    echo 'ERROR: Local API is ready, but the public health check failed. Check the reverse proxy.' >&2
    exit 1
fi
echo "==> bkb-server deployed and running: https://bitcoinknowledge.dev/health"
