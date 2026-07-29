#!/usr/bin/env bash
# Installs the Linux server binary and creates a systemd service. Run as root or with sudo.
set -Eeuo pipefail

REPOSITORY="${CROSS233_REPOSITORY:-neko233-com/cross233}"
VERSION="${CROSS233_VERSION:-latest}"
INSTALL_DIR="${CROSS233_INSTALL_DIR:-/usr/local/bin}"
CONFIG_DIR="${CROSS233_CONFIG_DIR:-/etc/cross233}"
DATA_DIR="${CROSS233_DATA_DIR:-/var/lib/cross233}"
AUTH_KEY="${CROSS233_AUTH_KEY:-}"
CONTROL_PORT="${CROSS233_CONTROL_PORT:-7710}"
WEB_PORT="${CROSS233_WEB_PORT:-7711}"

fail() { printf 'cross233 install: %s\n' "$*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || fail "missing required command: $1"; }
download() {
  if command -v curl >/dev/null 2>&1; then curl -fL --retry 3 -o "$2" "$1"; else wget -qO "$2" "$1"; fi
}
as_root() {
  if [ "$(id -u)" -eq 0 ]; then "$@"; elif command -v sudo >/dev/null 2>&1; then sudo "$@"; else fail "run as root or install sudo"; fi
}

case "$(uname -s)" in Linux) ;; *) fail "server installer supports Linux only" ;; esac
case "$(uname -m)" in
  x86_64|amd64) ARCH="x86_64" ;;
  aarch64|arm64) ARCH="aarch64" ;;
  armv7l) ARCH="armv7" ;;
  *) fail "unsupported architecture: $(uname -m)" ;;
esac
need systemctl

if [ "$VERSION" = "latest" ]; then
  VERSION=$(curl -fsSL "https://api.github.com/repos/$REPOSITORY/releases/latest" | grep '"tag_name"' | head -1 | sed -E 's/.*"tag_name":\s*"([^"]+)".*/\1/')
fi

ASSET="cross233-${VERSION}-${ARCH}-unknown-linux-musl.tar.gz"
BASE_URL="https://github.com/$REPOSITORY/releases/download/$VERSION"
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

if ! download "$BASE_URL/$ASSET" "$TMP_DIR/$ASSET" 2>/dev/null; then
  fail "failed to download $ASSET; build from source: cargo build --release -p cross233-server"
fi

tar -xzf "$TMP_DIR/$ASSET" -C "$TMP_DIR"
[ -f "$TMP_DIR/cross233-server" ] || fail "binary not found in archive"
chmod 0755 "$TMP_DIR/cross233-server"

as_root install -d -m 0755 "$INSTALL_DIR" "$CONFIG_DIR" "$DATA_DIR"
as_root install -m 0755 "$TMP_DIR/cross233-server" "$INSTALL_DIR/cross233-server"

if [ -z "$AUTH_KEY" ]; then
  AUTH_KEY=$(openssl rand -hex 32 2>/dev/null || head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n')
fi
printf '%s\n' "$AUTH_KEY" | as_root tee "$DATA_DIR/auth.key" >/dev/null
as_root chmod 0600 "$DATA_DIR/auth.key"

if [ ! -f "$CONFIG_DIR/server.toml" ]; then
  as_root tee "$CONFIG_DIR/server.toml" >/dev/null <<EOF
bind = "0.0.0.0"
auth_key_file = "$DATA_DIR/auth.key"
cert_file = "$DATA_DIR/cert.pem"
key_file = "$DATA_DIR/key.pem"
control_port = $CONTROL_PORT
web_port = $WEB_PORT
port_min = 7712
port_max = 7720
qcp_port = 7713
EOF
fi

as_root tee /etc/systemd/system/cross233.service >/dev/null <<EOF
[Unit]
Description=cross233 reverse tunnel server
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=$INSTALL_DIR/cross233-server -c $CONFIG_DIR/server.toml
Restart=on-failure
RestartSec=3
NoNewPrivileges=true
PrivateTmp=true
ProtectHome=true
ProtectSystem=strict
ReadWritePaths=$DATA_DIR

[Install]
WantedBy=multi-user.target
EOF
as_root systemctl daemon-reload
as_root systemctl enable --now cross233

printf '\n%s\n' "=== cross233 server installed ==="
printf '%s\n' "Binary:   $INSTALL_DIR/cross233-server"
printf '%s\n' "Config:   $CONFIG_DIR/server.toml"
printf '%s\n' "Data:     $DATA_DIR"
printf '%s\n' "Auth key: $AUTH_KEY"
printf '%s\n' "Web UI:   http://<server-ip>:$WEB_PORT"
printf '%s\n' "Control:  port $CONTROL_PORT"
printf '%s\n' ""
printf '%s\n' "Service status: systemctl status cross233"
printf '%s\n' "Logs:           journalctl -u cross233 -f"
