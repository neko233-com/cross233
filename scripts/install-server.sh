#!/usr/bin/env bash
# Installs the Linux server binary and creates a systemd service. Run as root or with sudo.
set -Eeuo pipefail

REPOSITORY="${CROSS233_REPOSITORY:-neko233-com/cross233}"
VERSION="${CROSS233_VERSION:-v0.1.0}"
PASSWORD="${CROSS233_PASSWORD:-root}"
INSTALL_DIR="${CROSS233_INSTALL_DIR:-/usr/local/bin}"
CONFIG_DIR="${CROSS233_CONFIG_DIR:-/etc/cross233}"
DATA_DIR="${CROSS233_DATA_DIR:-/var/lib/cross233}"

fail() { printf 'cross233 install: %s\n' "$*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || fail "missing required command: $1"; }
download() {
  if command -v curl >/dev/null 2>&1; then curl -fL --retry 3 -o "$2" "$1"; else wget -qO "$2" "$1"; fi
}
as_root() {
  if [ "$(id -u)" -eq 0 ]; then "$@"; elif command -v sudo >/dev/null 2>&1; then sudo "$@"; else fail "run as root or install sudo"; fi
}

case "$(uname -s)" in Linux) ;; *) fail "server installer supports Linux only" ;; esac
case "$(uname -m)" in x86_64|amd64) ARCH=amd64 ;; aarch64|arm64) ARCH=arm64 ;; *) fail "unsupported architecture: $(uname -m)" ;; esac
[ -n "$PASSWORD" ] || fail "CROSS233_PASSWORD cannot be empty"
printf '%s' "$PASSWORD" | grep -q '[^A-Za-z0-9._~@%+=,:!-]' && fail "installer password supports only letters, digits, and . _ ~ @ % + = , : ! -"
need systemctl; need sha256sum

ASSET="cross233-server-linux-$ARCH"
BASE_URL="https://github.com/$REPOSITORY/releases/download/$VERSION"
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT
download "$BASE_URL/$ASSET" "$TMP_DIR/$ASSET"
download "$BASE_URL/checksums.txt" "$TMP_DIR/checksums.txt"
(cd "$TMP_DIR" && grep "  $ASSET$" checksums.txt | sha256sum -c -) || fail "checksum verification failed"
chmod 0755 "$TMP_DIR/$ASSET"

as_root install -d -m 0755 "$INSTALL_DIR" "$CONFIG_DIR" "$DATA_DIR"
as_root install -m 0755 "$TMP_DIR/$ASSET" "$INSTALL_DIR/cross233-server"
{
  printf 'CROSS233_PASSWORD=%s\n' "$PASSWORD"
  printf 'CROSS233_BIND=0.0.0.0\n'
} | as_root tee "$CONFIG_DIR/server.env" >/dev/null
as_root chmod 0600 "$CONFIG_DIR/server.env"
as_root tee /etc/systemd/system/cross233.service >/dev/null <<EOF
[Unit]
Description=cross233 reverse tunnel server
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
EnvironmentFile=$CONFIG_DIR/server.env
ExecStart=$INSTALL_DIR/cross233-server -bind \${CROSS233_BIND} -password \${CROSS233_PASSWORD} -cert $DATA_DIR/cross233-cert.pem -key $DATA_DIR/cross233-key.pem
Restart=on-failure
RestartSec=3
NoNewPrivileges=true
PrivateTmp=true
ProtectHome=true
ProtectSystem=full
ReadWritePaths=$DATA_DIR

[Install]
WantedBy=multi-user.target
EOF
as_root systemctl daemon-reload
as_root systemctl enable --now cross233

printf '%s\n' "cross233 installed: $INSTALL_DIR/cross233-server"
printf '%s\n' "management: https://SERVER:7711"
printf '%s\n' "control/public ports: 7710, 7712-7720"
if [ "$PASSWORD" = root ]; then printf '%s\n' 'WARNING: default password root is active. Set CROSS233_PASSWORD before production install.' >&2; fi
